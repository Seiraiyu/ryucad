// Writes the STEP files beside it with OCCT 7.8, for ryustep's tests.
//
// In the devcontainer, with OCCT's data exchange headers (which the image
// doesn't install), from this folder:
//
//   sudo apt-get install -y libocct-data-exchange-dev libocct-ocaf-dev
//   g++ -std=c++17 -I/usr/include/opencascade gen.cpp -o /tmp/gen \
//     -lTKernel -lTKMath -lTKG2d -lTKG3d -lTKGeomBase -lTKBRep -lTKTopAlgo \
//     -lTKPrim -lTKBO -lTKDESTEP -lTKXSBase -lTKLCAF -lTKXCAF -lTKCAF \
//     -lTKCDF -lTKBool -lTKGeomAlgo -lTKShHealing -lTKOffset
//   /tmp/gen
//
// Only the time stamps in the headers change from run to run.
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_NurbsConvert.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepOffsetAPI_MakeOffsetShape.hxx>
#include <gp_Elips.hxx>
#include <Geom_BSplineCurve.hxx>
#include <Geom_BSplineSurface.hxx>
#include <GeomAPI_PointsToBSpline.hxx>
#include <GeomAPI_PointsToBSplineSurface.hxx>
#include <TColgp_Array1OfPnt.hxx>
#include <TColgp_Array2OfPnt.hxx>
#include <STEPControl_Writer.hxx>
#include <STEPCAFControl_Writer.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <TDocStd_Document.hxx>
#include <TDataStd_Name.hxx>
#include <Interface_Static.hxx>
#include <gp_Ax2.hxx>
#include <gp_Trsf.hxx>
#include <TopLoc_Location.hxx>
#include <TopoDS_Shape.hxx>
#include <string>

static void write(const TopoDS_Shape& s, const char* file, int schema, const char* unit) {
  Interface_Static::SetIVal("write.step.schema", schema);
  Interface_Static::SetCVal("write.step.unit", unit);
  Interface_Static::SetCVal("write.step.product.name", "part");
  STEPControl_Writer w;
  w.Transfer(s, STEPControl_AsIs);
  w.Write(file);
}

int main() {
  // The first writer sets up the parameters; set before it, they are lost.
  { STEPControl_Writer init; }
  TopoDS_Shape box = BRepPrimAPI_MakeBox(10., 20., 30.).Shape();
  TopoDS_Shape cyl = BRepPrimAPI_MakeCylinder(5., 12.).Shape();
  TopoDS_Shape sph = BRepPrimAPI_MakeSphere(7.).Shape();
  TopoDS_Shape tor = BRepPrimAPI_MakeTorus(10., 3.).Shape();
  TopoDS_Shape cone = BRepPrimAPI_MakeCone(6., 2., 9.).Shape();
  TopoDS_Shape nurbs = BRepBuilderAPI_NurbsConvert(cyl).Shape();
  gp_Trsf t; t.SetTranslation(gp_Vec(5., 10., -1.));
  TopoDS_Shape hole = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(5., 10., -1.), gp::DZ()), 3., 32.).Shape();
  TopoDS_Shape plate = BRepAlgoAPI_Cut(box, hole).Shape();
  write(box, "box_ap242.stp", 5, "MM");
  write(box, "box_ap214.stp", 4, "MM");
  write(box, "box_ap203.stp", 3, "MM");
  write(box, "box_inch.stp", 5, "INCH");
  write(cyl, "cylinder.stp", 5, "MM");
  write(sph, "sphere.stp", 5, "MM");
  write(tor, "torus.stp", 5, "MM");
  write(cone, "cone.stp", 5, "MM");
  write(nurbs, "nurbs_cylinder.stp", 5, "MM");
  write(plate, "plate_hole.stp", 5, "MM");

  // An elliptic prism: an ellipse swept along z.
  gp_Elips el(gp_Ax2(gp_Pnt(0, 0, 0), gp::DZ()), 8., 3.);
  TopoDS_Edge ee = BRepBuilderAPI_MakeEdge(el);
  TopoDS_Face ef = BRepBuilderAPI_MakeFace(BRepBuilderAPI_MakeWire(ee).Wire());
  write(BRepPrimAPI_MakePrism(ef, gp_Vec(0, 0, 5.)).Shape(), "elliptic_prism.stp", 5, "MM");

  // A spline profile in the xz half-plane revolved half a turn about z.
  TColgp_Array1OfPnt pts(1, 4);
  pts(1) = gp_Pnt(2, 0, 0); pts(2) = gp_Pnt(5, 0, 2); pts(3) = gp_Pnt(4, 0, 5); pts(4) = gp_Pnt(1, 0, 8);
  Handle(Geom_BSplineCurve) prof = GeomAPI_PointsToBSpline(pts).Curve();
  TopoDS_Edge pe = BRepBuilderAPI_MakeEdge(prof);
  TopoDS_Edge top = BRepBuilderAPI_MakeEdge(gp_Pnt(1, 0, 8), gp_Pnt(0, 0, 8));
  TopoDS_Edge ax = BRepBuilderAPI_MakeEdge(gp_Pnt(0, 0, 8), gp_Pnt(0, 0, 0));
  TopoDS_Edge bot = BRepBuilderAPI_MakeEdge(gp_Pnt(0, 0, 0), gp_Pnt(2, 0, 0));
  TopoDS_Face pf = BRepBuilderAPI_MakeFace(BRepBuilderAPI_MakeWire(pe, top, ax, bot).Wire());
  write(BRepPrimAPI_MakeRevol(pf, gp_Ax1(gp_Pnt(0, 0, 0), gp::DZ()), M_PI).Shape(), "spline_revolve.stp", 5, "MM");

  // A block with a cavity: two shells, one of them a void.
  TopoDS_Shape inner = BRepPrimAPI_MakeBox(gp_Pnt(2, 2, 2), 4., 4., 4.).Shape();
  TopoDS_Shape outer = BRepPrimAPI_MakeBox(10., 10., 10.).Shape();
  write(BRepAlgoAPI_Cut(outer, inner).Shape(), "cavity.stp", 5, "MM");

  // A spline sheet offset by 1: an open shell on an offset surface.
  TColgp_Array2OfPnt grid(1, 4, 1, 4);
  for (int i = 1; i <= 4; i++)
    for (int j = 1; j <= 4; j++)
      grid(i, j) = gp_Pnt(3. * i, 3. * j, ((i + j) % 3) * 0.7);
  Handle(Geom_BSplineSurface) sheet = GeomAPI_PointsToBSplineSurface(grid).Surface();
  TopoDS_Face sf = BRepBuilderAPI_MakeFace(sheet, 1e-7);
  BRepOffsetAPI_MakeOffsetShape off;
  off.PerformBySimple(sf, 1.);
  write(off.Shape(), "offset_sheet.stp", 5, "MM");

  // An assembly: one box part placed twice, and a cylinder part, by XDE.
  Handle(XCAFApp_Application) app = XCAFApp_Application::GetApplication();
  Handle(TDocStd_Document) doc;
  app->NewDocument("MDTV-XCAF", doc);
  Handle(XCAFDoc_ShapeTool) st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
  TDF_Label asmL = st->NewShape();
  TDataStd_Name::Set(asmL, "assembly");
  TDF_Label boxL = st->AddShape(box, Standard_False);
  TDataStd_Name::Set(boxL, "block");
  TDF_Label cylL = st->AddShape(cyl, Standard_False);
  TDataStd_Name::Set(cylL, "pin");
  gp_Trsf a; a.SetTranslation(gp_Vec(100., 0., 0.));
  gp_Trsf b; b.SetRotation(gp_Ax1(gp_Pnt(0,0,0), gp::DZ()), M_PI / 2); b.SetTranslationPart(gp_Vec(0., 50., 0.));
  st->AddComponent(asmL, boxL, TopLoc_Location(gp_Trsf()));
  st->AddComponent(asmL, boxL, TopLoc_Location(a));
  st->AddComponent(asmL, cylL, TopLoc_Location(b));
  st->UpdateAssemblies();
  Interface_Static::SetIVal("write.step.schema", 5);
  Interface_Static::SetCVal("write.step.unit", "MM");
  STEPCAFControl_Writer cw;
  cw.Transfer(doc, STEPControl_AsIs);
  cw.Write("assembly.stp");
  return 0;
}
